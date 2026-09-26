#[derive(Debug, Clone)]
pub(super) struct Viewport {
    pub(super) start: f64,
    pub(super) span: f64,
    horizon: f64,
}
impl Viewport {
    pub(super) fn new(finish: f64) -> Self {
        let horizon = finish.max(1.0);
        Self {
            start: 0.0,
            span: horizon.min(48.0),
            horizon,
        }
    }
    pub(super) fn restore(&mut self, previous: &Self) {
        self.span = if previous.span == previous.horizon {
            self.horizon
        } else {
            previous.span.min(self.horizon)
        };
        self.start = previous.start.min(self.horizon - self.span);
    }
    pub(super) fn end(&self) -> f64 {
        (self.start + self.span).min(self.horizon)
    }
    pub(super) fn pan(&mut self, right: bool) {
        let step = self.span / 4.0;
        self.start = if right {
            (self.start + step).min(self.horizon - self.span)
        } else {
            (self.start - step).max(0.0)
        };
    }
    pub(super) fn home(&mut self) {
        self.start = 0.0;
    }
    pub(super) fn end_of_plan(&mut self) {
        self.start = self.horizon - self.span;
    }
    pub(super) fn fit(&mut self) {
        self.start = 0.0;
        self.span = self.horizon;
    }
    pub(super) fn zoom(&mut self, inward: bool) {
        self.span = if inward {
            (self.span / 2.0).max(1.0).min(self.horizon)
        } else if self.span > self.horizon / 2.0 {
            self.horizon
        } else {
            self.span * 2.0
        };
        self.start = self.start.min(self.horizon - self.span);
    }
    pub(super) fn cell(&self, hours: f64, width: usize) -> usize {
        (((hours - self.start) / self.span).clamp(0.0, 1.0) * width.saturating_sub(1) as f64)
            .round() as usize
    }
    pub(super) fn contains(&self, start: f64, end: f64) -> bool {
        start <= self.end() && end >= self.start
    }
}
