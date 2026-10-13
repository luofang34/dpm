/// The visible window over elapsed hours from the clock reading: recorded history lies before
/// zero, back to `floor`, and the remaining schedule after it, up to `horizon`.
#[derive(Debug, Clone)]
pub(super) struct Viewport {
    pub(super) start: f64,
    pub(super) span: f64,
    floor: f64,
    horizon: f64,
}
impl Viewport {
    /// A window opening on the remaining work with up to a quarter of it on the recent history,
    /// or on the history alone when nothing remains.
    pub(super) fn new(history: f64, finish: f64) -> Self {
        let floor = history.min(0.0);
        let horizon = finish.max(if floor < 0.0 { 0.0 } else { 1.0 });
        let span = (horizon - floor).min(48.0);
        let start = if finish > 0.0 {
            floor.max(-span / 4.0)
        } else {
            horizon - span
        };
        Self {
            start,
            span,
            floor,
            horizon,
        }
    }
    fn length(&self) -> f64 {
        self.horizon - self.floor
    }
    fn latest_start(&self) -> f64 {
        self.horizon - self.span
    }
    pub(super) fn restore(&mut self, previous: &Self) {
        self.span = if previous.span == previous.length() {
            self.length()
        } else {
            previous.span.min(self.length())
        };
        self.start = previous.start.max(self.floor).min(self.latest_start());
    }
    pub(super) fn end(&self) -> f64 {
        (self.start + self.span).min(self.horizon)
    }
    pub(super) fn pan(&mut self, right: bool) {
        let step = self.span / 4.0;
        self.start = if right {
            (self.start + step).min(self.latest_start())
        } else {
            (self.start - step).max(self.floor)
        };
    }
    pub(super) fn home(&mut self) {
        self.start = self.floor;
    }
    pub(super) fn end_of_plan(&mut self) {
        self.start = self.latest_start();
    }
    pub(super) fn fit(&mut self) {
        self.start = self.floor;
        self.span = self.length();
    }
    pub(super) fn zoom(&mut self, inward: bool) {
        self.span = if inward {
            (self.span / 2.0).max(1.0).min(self.length())
        } else if self.span > self.length() / 2.0 {
            self.length()
        } else {
            self.span * 2.0
        };
        self.start = self.start.max(self.floor).min(self.latest_start());
    }
    pub(super) fn cell(&self, hours: f64, width: usize) -> usize {
        (((hours - self.start) / self.span).clamp(0.0, 1.0) * width.saturating_sub(1) as f64)
            .round() as usize
    }
    pub(super) fn contains(&self, start: f64, end: f64) -> bool {
        start <= self.end() && end >= self.start
    }
}
